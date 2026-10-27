var nodes ;
var nodes_running ;
var nodes_loaded ;
var graph ;
var edges ;
var root_q ;
var max_nodes = 100 ;
var excessive_nodes ;

var relations = [
	{ prop : 'P22' , name : 'father' , to_q : false , edge_color : '#3923D6' } ,
	{ prop : 'P25' , name : 'mother' , to_q : false , edge_color : '#FF4848' } ,
	{ prop : 'P40' , name : 'child' , to_q : false , edge_color : '#888888' }
] ;

function updateLoadingStatus () {
	var h = "" ;
	h += nodes_loaded + " people loaded. " ;
	if ( nodes_loaded >= max_nodes ) h += "<span style='color:red'>Maximum number of people reached; graph is incomplete.</span> " ;
	if ( nodes_running > 0 ) h += nodes_running + " still loading. " ;
	$('#loading_status').html(h).show() ;
}

function scanWikidata ( q , q2 , to_q , edge_color ) {
	q = q.toUpperCase() ;
	if ( undefined !== nodes[q] ) {
		addEdge ( q , q2 , to_q , edge_color ) ;
		return ;
	}
	if ( nodes_loaded >= max_nodes ) {
		excessive_nodes = true ;
		return ;
	}
	if ( edge_color === undefined ) edge_color = '#888888' ;
	nodes_loaded++ ;
	updateLoadingStatus () ;
	nodes[q] = { running : true } ;
	nodes_running++ ;
	$.getJSON ( '//www.wikidata.org/w/api.php?callback=?' , {
		action : 'wbgetentities' ,
		ids : q ,
		languages : 'en' ,
		props : 'info|aliases|labels|descriptions|claims' ,
		format : 'json'
	} , function ( data ) {
		nodes_running-- ;
		nodes[q].running = false ;
		if ( undefined !== data.entities['-1'] ) {
			showGraph() ;
			return ;
		}
		var o ;
		$.each ( data.entities , function ( k , v ) { o = v } ) ;
		if ( o.labels != undefined && undefined !== o.labels.en ) nodes[q].title = o.labels.en.value ;
		else nodes[q].title = q ;
		
		nodes[q].wd_url = '//www.wikidata.org/wiki/' + q ;
		nodes[q].n = graph.newNode({q:q,label: nodes[q].title});
		nodes[q].wd = data ;
		addEdge ( q , q2 , to_q , edge_color ) ;
		if ( o.claims !== undefined ) {
			$.each ( relations , function ( k , v ) {
				if ( undefined === o.claims[v.prop] ) return ;
				$.each ( o.claims[v.prop] , function ( k3 , v3 ) {
					var nq = v3.mainsnak.datavalue.value['numeric-id'] ;
					scanWikidata ( 'Q'+nq , q , v.to_q , v.edge_color ) ;
				} ) ;
			} ) ;
		}
		showGraph() ;
	} ) ;
}

function addEdge ( q , q2 , to_q , edge_color ) {
	if ( undefined === q ) return ;
	if ( undefined === q2 ) return ;
	edges.push ( { q:q , q2:q2 , to_q:to_q , col:edge_color } ) ;
}

function showGraph () {
	if ( nodes_running > 0 ) return ;
	updateLoadingStatus() ;
	$.each ( edges , function ( k , v ) {
		if ( undefined === nodes[v.q] ) return ;
		if ( undefined === nodes[v.q2] ) return ;
		if ( v.to_q ) graph.newEdge ( nodes[v.q2].n , nodes[v.q].n , {color: v.col } ) ;
		else graph.newEdge ( nodes[v.q].n , nodes[v.q2].n , {color: v.col } ) ;
	} ) ;
	
	var springy = jQuery('#graph').springy({
		graph: graph,
		nodeSelected: function(node){
			var q = node.data.q ;
			var title = node.data.label + " (" + q + ")" ;
			$('#last_sel a').attr('href',nodes[q].wd_url).html(title) ;
			if ( excessive_nodes ) {
				$('#recenter').html("| <a href='#' onclick='loadQ(\""+q+"\");return false'>Load partial graph centered on this person</a>").show() ;
			}
			$('#last_sel').show();
//			window.open(nodes[q].wd_url,'_blank');
		}
	});
}

function loadQ ( q ) {
	if ( undefined === q ) q = $('#q').val() ;
	q = q.toUpperCase() ;
	$('#q').val(q) ;
	root_q = q ;
	var pmurl = '?q=' + q ;
	$('#permalink').attr('href',pmurl) ;
	$('#permalink_container').show() ;
	$('#result_container').show() ;
	$('#recenter').hide() ;
	
	nodes_loaded = 0 ;
	nodes_running = 0 ;
	excessive_nodes = false ;
	graph = new Graph();
	nodes = {} ;
	edges = [] ;
	scanWikidata ( q ) ;
}

function initializeFromParameters () {
	var params = getUrlVars() ;
	if ( undefined !== params.q ) loadQ ( params.q ) ;
}

$(document).ready ( function () {
	loadMenuBarAndContent ( { toolname : 'GeneaWiki' , meta : 'GeneaWiki' , content : 'form.html' , run : function () {
		initializeFromParameters() ;
		$('#q').focus() ;
	} } ) ;
} ) ;
